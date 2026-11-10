import {useRef,useState} from 'react';
import {roomPreferences,updateRoom} from './experienceState';

export function useRoomDraft(key:string):[string,(value:string|((previous:string)=>string))=>void]{
  const [state,setState]=useState(()=>({key,text:roomPreferences(key)?.draft||''}));
  const current=useRef(state);current.current=state.key===key?state:{key,text:roomPreferences(key)?.draft||''};
  const value=current.current;
  const setText=(next:string|((previous:string)=>string))=>{
    if(current.current.key!==key)return;
    const text=typeof next==='function'?next(current.current.text):next;
    current.current={key,text};updateRoom(key,{draft:text});setState({key,text});
  };
  return [value.text,setText];
}
